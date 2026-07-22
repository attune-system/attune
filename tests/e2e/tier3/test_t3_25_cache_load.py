"""T3.25: Opt-in 200,000-record cache ingest and pinned full-scan load scenario."""

from __future__ import annotations

import time

import pytest

from e2e.cache_helpers import (
    CACHE_OWNER_TYPE,
    assert_error_status,
    cache_metric_lines,
    cache_namespace,
    cache_refresh_ref,
    create_metrics_artifact,
    generation_id,
)
from helpers import AttuneClient

pytestmark = pytest.mark.usefixtures("cache_e2e_retention_config")


RECORD_COUNT = 200_000
CHUNK_SIZE = 2_000
CHUNK_COUNT = RECORD_COUNT // CHUNK_SIZE


def _records(start: int, count: int, *, revision: str = "load") -> list[dict]:
    """Generate fixed-width IDs so ordering and retry payloads are deterministic."""
    return [
        {
            "external_id": f"load-{index:09d}",
            "value": {
                "ordinal": index,
                "revision": revision,
                "source": "cache-e2e-load",
            },
        }
        for index in range(start, start + count)
    ]


@pytest.mark.tier3
@pytest.mark.integration
@pytest.mark.cache
@pytest.mark.performance
@pytest.mark.slow
@pytest.mark.timeout(1800)
def test_cache_200k_streamed_ingestion_and_pinned_full_scan(
    client: AttuneClient, pack_ref: str
):
    """Run only in the scheduled/manual performance gate, never normal E2E."""
    request_count = 0
    error_count = 0
    started = time.monotonic()
    with cache_namespace(
        client,
        owner_ref=pack_ref,
        prefix="load-200k",
        policy={
            "max_records_per_generation": RECORD_COUNT,
            "max_generation_bytes": 256 * 1024 * 1024,
            "max_retained_generations": 2,
        },
    ) as namespace:
        refresh_id = cache_refresh_ref("load-200k")
        generation = generation_id(
            client.cache_create_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                client_refresh_id=refresh_id,
                expected_active_generation_id=None,
                expected_chunk_count=CHUNK_COUNT,
                expected_record_count=RECORD_COUNT,
                source_revision="cache-e2e-200k-v1",
            )
        )
        request_count += 1

        reported_size_bytes = None
        ingestion_started = time.monotonic()
        for chunk_index in range(CHUNK_COUNT):
            chunk = _records(chunk_index * CHUNK_SIZE, CHUNK_SIZE)
            try:
                accepted = client.cache_upload_chunk(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    generation_id=generation,
                    chunk_index=chunk_index,
                    entries=chunk,
                )
                request_count += 1
                reported_size_bytes = accepted.get(
                    "size_bytes", accepted.get("generation_size_bytes", reported_size_bytes)
                )
            except Exception:
                error_count += 1
                raise

            # Simulated retry of an already accepted numbered chunk: this must
            # be idempotent and must not add another 2,000 records.
            if chunk_index == CHUNK_COUNT // 2:
                replayed = client.cache_upload_chunk(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    generation_id=generation,
                    chunk_index=chunk_index,
                    entries=chunk,
                )
                request_count += 1
                assert generation_id(replayed) == generation

            if chunk_index % 10 == 0:
                health = client.get("/health")
                request_count += 1
                assert health.status_code == 200, (
                    f"health became unavailable during chunk {chunk_index}: {health.text}"
                )

        assert isinstance(reported_size_bytes, int) and reported_size_bytes > 0, (
            f"ingest did not report an authoritative byte count: generation={generation}"
        )
        sealed = client.cache_seal_generation(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            generation_id=generation,
            expected_chunk_count=CHUNK_COUNT,
            expected_record_count=RECORD_COUNT,
            expected_size_bytes=reported_size_bytes,
        )
        request_count += 1
        assert sealed.get("record_count") == RECORD_COUNT, (
            f"seal record count mismatch: namespace={namespace} refresh={refresh_id}"
        )
        assert sealed.get("size_bytes") == reported_size_bytes, (
            f"seal byte count changed: namespace={namespace} refresh={refresh_id}"
        )
        promoted = client.cache_promote_generation(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            generation_id=generation,
            expected_active_generation_id=None,
        )
        request_count += 1
        assert generation_id(promoted) == generation
        assert client.get("/health").status_code == 200
        request_count += 1

        point = client.cache_lookup(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            external_id="load-000100000",
        )
        request_count += 1
        assert point.get("item", point.get("entry"))["value"]["ordinal"] == 100_000
        many = client.cache_lookup_many(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            external_ids=["load-000000000", "load-000199999", "load-missing"],
        )
        request_count += 1
        assert [item["external_id"] for item in many.get("items", many.get("entries", []))] == [
            "load-000000000",
            "load-000199999",
        ]
        assert many.get("missing_external_ids", many.get("missing_ids")) == ["load-missing"]

        scan_started = time.monotonic()
        page = client.cache_scan(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            page_size=CHUNK_SIZE,
        )
        request_count += 1
        assert generation_id(page) == generation
        seen: set[str] = set()
        first_id = last_id = None
        while True:
            items = page.get("items", page.get("entries", []))
            for item in items:
                external_id = item["external_id"]
                first_id = first_id or external_id
                last_id = external_id
                assert external_id not in seen, (
                    f"duplicate during 200k scan: {external_id} generation={generation}"
                )
                seen.add(external_id)

            cursor = page.get("next_cursor")
            if not cursor:
                break
            if len(seen) == CHUNK_SIZE:
                # Promotion during a traversal must not change the old cursor's
                # generation, even when the fresh active snapshot is tiny.
                changed_refresh = cache_refresh_ref("load-small-refresh")
                changed_generation = generation_id(
                    client.cache_create_generation(
                        owner_type=CACHE_OWNER_TYPE,
                        owner_ref=pack_ref,
                        namespace=namespace,
                        client_refresh_id=changed_refresh,
                        expected_active_generation_id=generation,
                        expected_chunk_count=1,
                        expected_record_count=3,
                        source_revision="cache-e2e-200k-v2",
                    )
                )
                changed_records = _records(0, 3, revision="promoted-during-scan")
                client.cache_upload_chunk(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    generation_id=changed_generation,
                    chunk_index=0,
                    entries=changed_records,
                )
                client.cache_seal_generation(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    generation_id=changed_generation,
                    expected_chunk_count=1,
                    expected_record_count=3,
                )
                client.cache_promote_generation(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    generation_id=changed_generation,
                    expected_active_generation_id=generation,
                )
                request_count += 4

            page = client.cache_scan(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                page_size=CHUNK_SIZE,
                generation_id=generation,
                cursor=cursor,
            )
            request_count += 1
            assert generation_id(page) == generation, (
                f"pinned 200k scan changed generation after promotion: {namespace}"
            )

        assert len(seen) == RECORD_COUNT
        assert first_id == "load-000000000"
        assert last_id == "load-000199999"

        fresh_page = client.cache_scan(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            page_size=10,
        )
        request_count += 1
        assert generation_id(fresh_page) != generation
        assert [
            item["value"]["revision"]
            for item in fresh_page.get("items", fresh_page.get("entries", []))
        ] == ["promoted-during-scan"] * 3

        with cache_namespace(
            client,
            owner_ref=pack_ref,
            prefix="load-quota",
            policy={"max_records_per_generation": 1, "max_generation_bytes": 65536},
        ) as quota_namespace:
            quota_generation = generation_id(
                client.cache_create_generation(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=quota_namespace,
                    client_refresh_id=cache_refresh_ref("load-quota"),
                    expected_active_generation_id=None,
                    expected_chunk_count=1,
                    expected_record_count=2,
                )
            )
            assert_error_status(
                lambda: client.cache_upload_chunk(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=quota_namespace,
                    generation_id=quota_generation,
                    chunk_index=0,
                    entries=_records(0, 2),
                ),
                expected={400, 409, 413, 422},
                assertion=f"200k quota rejection namespace={quota_namespace}",
            )
            request_count += 2
            error_count += 1

        metrics = {
            "namespace": namespace,
            "generation_id": generation,
            "records": RECORD_COUNT,
            "chunks": CHUNK_COUNT,
            "chunk_size": CHUNK_SIZE,
            "ingestion_seconds": round(time.monotonic() - ingestion_started, 3),
            "full_scan_seconds": round(time.monotonic() - scan_started, 3),
            "wall_clock_seconds": round(time.monotonic() - started, 3),
            "request_count": request_count,
            "error_count": error_count,
            "service_telemetry": cache_metric_lines(client),
        }
        metrics_artifact = create_metrics_artifact(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            metrics=metrics,
        )
        assert metrics_artifact.get("id"), f"load metrics artifact missing id: {metrics_artifact}"
