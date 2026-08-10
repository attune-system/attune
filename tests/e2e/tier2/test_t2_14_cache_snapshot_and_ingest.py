"""T2.14: Cache ingest retry, immutable snapshots, and cursor safety."""

from concurrent.futures import ThreadPoolExecutor

import pytest

from e2e.cache_helpers import (
    CACHE_OWNER_TYPE,
    assert_error_status,
    assertion_id,
    cache_entries,
    cache_namespace,
    cache_refresh_ref,
    generation_id,
    publish_generation,
    response_status,
    scan_all,
    wait_for_cursor_expiry,
)
from helpers import AttuneClient
from helpers.client_wrapper import CacheApiError

pytestmark = pytest.mark.usefixtures("cache_e2e_retention_config")


def _assert_active_record(
    client: AttuneClient, *, pack_ref: str, namespace: str, expected_ordinal: int
) -> None:
    record = client.cache_lookup(
        owner_type=CACHE_OWNER_TYPE,
        owner_ref=pack_ref,
        namespace=namespace,
        external_id="base-000000",
    )
    assert record.get("item", record.get("entry"))["value"]["ordinal"] == expected_ordinal


@pytest.mark.tier2
@pytest.mark.integration
@pytest.mark.cache
def test_cache_chunk_replay_and_invalid_staging_never_replace_active(
    client: AttuneClient, pack_ref: str
):
    with cache_namespace(
        client,
        owner_ref=pack_ref,
        prefix="ingest",
        policy={
            "max_records_per_generation": 4,
            "max_generation_bytes": 65536,
            "max_staging_generations": 5,
        },
    ) as namespace:
        base_generation, base_refresh = publish_generation(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            entries=cache_entries(2, prefix="base"),
            expected_active_generation_id=None,
        )
        assertion = assertion_id(namespace, base_refresh)
        _assert_active_record(
            client, pack_ref=pack_ref, namespace=namespace, expected_ordinal=0
        )

        replay_refresh = cache_refresh_ref("identical-replay")
        replay = client.cache_create_generation(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            client_refresh_id=replay_refresh,
            expected_active_generation_id=base_generation,
            expected_chunk_count=2,
            expected_record_count=2,
        )
        replay_generation = generation_id(replay)
        first_chunk = cache_entries(1, prefix="replay")
        accepted = client.cache_upload_chunk(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            generation_id=replay_generation,
            chunk_index=0,
            entries=first_chunk,
        )
        replayed = client.cache_upload_chunk(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            generation_id=replay_generation,
            chunk_index=0,
            entries=first_chunk,
        )
        assert generation_id(accepted) == generation_id(replayed) == replay_generation, (
            assertion_id(namespace, replay_refresh)
        )
        assert_error_status(
            lambda: client.cache_upload_chunk(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                generation_id=replay_generation,
                chunk_index=0,
                entries=cache_entries(1, prefix="conflicting"),
            ),
            expected={400, 409, 422},
            assertion=f"conflicting replay {assertion_id(namespace, replay_refresh)}",
        )
        _assert_active_record(
            client, pack_ref=pack_ref, namespace=namespace, expected_ordinal=0
        )

        missing_refresh = cache_refresh_ref("missing-chunk")
        missing_generation = generation_id(
            client.cache_create_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                client_refresh_id=missing_refresh,
                expected_active_generation_id=base_generation,
                expected_chunk_count=2,
                expected_record_count=2,
            )
        )
        client.cache_upload_chunk(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            generation_id=missing_generation,
            chunk_index=0,
            entries=cache_entries(1, prefix="missing"),
        )
        assert_error_status(
            lambda: client.cache_seal_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                generation_id=missing_generation,
                expected_chunk_count=2,
                expected_record_count=2,
            ),
            expected={400, 409, 422},
            assertion=f"missing chunk {assertion_id(namespace, missing_refresh)}",
        )

        duplicate_refresh = cache_refresh_ref("duplicate-id")
        duplicate_generation = generation_id(
            client.cache_create_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                client_refresh_id=duplicate_refresh,
                expected_active_generation_id=base_generation,
                expected_chunk_count=1,
                expected_record_count=2,
            )
        )
        duplicate_entry = {"external_id": "duplicate-id", "value": {"ordinal": 1}}
        assert_error_status(
            lambda: client.cache_upload_chunk(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                generation_id=duplicate_generation,
                chunk_index=0,
                entries=[duplicate_entry, duplicate_entry],
            ),
            expected={400, 409, 422},
            assertion=f"duplicate id {assertion_id(namespace, duplicate_refresh)}",
        )

        count_refresh = cache_refresh_ref("incorrect-count")
        count_generation = generation_id(
            client.cache_create_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                client_refresh_id=count_refresh,
                expected_active_generation_id=base_generation,
                expected_chunk_count=1,
                expected_record_count=3,
            )
        )
        client.cache_upload_chunk(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            generation_id=count_generation,
            chunk_index=0,
            entries=cache_entries(1, prefix="incorrect-count"),
        )
        assert_error_status(
            lambda: client.cache_seal_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                generation_id=count_generation,
                expected_chunk_count=1,
                expected_record_count=3,
            ),
            expected={400, 409, 422},
            assertion=f"incorrect count {assertion_id(namespace, count_refresh)}",
        )

        quota_refresh = cache_refresh_ref("quota")
        quota_generation = generation_id(
            client.cache_create_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                client_refresh_id=quota_refresh,
                expected_active_generation_id=base_generation,
                expected_chunk_count=1,
                expected_record_count=5,
            )
        )
        quota_entries = cache_entries(5, prefix="quota")
        try:
            client.cache_upload_chunk(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                generation_id=quota_generation,
                chunk_index=0,
                entries=quota_entries,
            )
        except CacheApiError as error:
            assert response_status(error) in {400, 409, 413, 422}, (
                f"quota upload {assertion_id(namespace, quota_refresh)}: {error.body}"
            )
        else:
            assert_error_status(
                lambda: client.cache_seal_generation(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    generation_id=quota_generation,
                    expected_chunk_count=1,
                    expected_record_count=5,
                ),
                expected={400, 409, 413, 422},
                assertion=f"quota seal {assertion_id(namespace, quota_refresh)}",
            )

        _assert_active_record(
            client, pack_ref=pack_ref, namespace=namespace, expected_ordinal=0
        )
        active_generation, active_entries, _ = scan_all(
            client, owner_ref=pack_ref, namespace=namespace, page_size=1
        )
        assert active_generation == base_generation, assertion
        assert [entry["external_id"] for entry in active_entries] == [
            "base-000000",
            "base-000001",
        ], assertion


@pytest.mark.tier2
@pytest.mark.integration
@pytest.mark.cache
def test_cache_scan_stays_pinned_across_promotion(
    client: AttuneClient, pack_ref: str
):
    with cache_namespace(client, owner_ref=pack_ref, prefix="snapshot") as namespace:
        old_records = cache_entries(24, prefix="customer", revision="old")
        old_generation, old_refresh = publish_generation(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            entries=old_records,
            expected_active_generation_id=None,
            chunk_size=6,
        )
        assertion = assertion_id(namespace, old_refresh)
        first_page = client.cache_scan(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            page_size=5,
        )
        cursor = first_page.get("next_cursor")
        assert cursor, f"First page omitted cursor: {assertion}"
        assert generation_id(first_page) == old_generation, assertion

        new_records = cache_entries(24, prefix="customer", revision="new")
        new_generation, new_refresh = publish_generation(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            entries=new_records,
            expected_active_generation_id=old_generation,
            chunk_size=6,
        )
        assert new_generation != old_generation, assertion_id(namespace, new_refresh)

        pinned_entries = list(first_page.get("items", first_page.get("entries", [])))
        while cursor:
            page = client.cache_scan(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                page_size=5,
                generation_id=old_generation,
                cursor=cursor,
            )
            assert generation_id(page) == old_generation, assertion
            pinned_entries.extend(page.get("items", page.get("entries", [])))
            cursor = page.get("next_cursor")
        assert [entry["value"]["revision"] for entry in pinned_entries] == ["old"] * 24, assertion
        assert len({entry["external_id"] for entry in pinned_entries}) == 24, assertion

        fresh_generation, fresh_entries, _ = scan_all(
            client, owner_ref=pack_ref, namespace=namespace, page_size=4
        )
        assert fresh_generation == new_generation, assertion_id(namespace, new_refresh)
        assert [entry["value"]["revision"] for entry in fresh_entries] == ["new"] * 24, (
            assertion_id(namespace, new_refresh)
        )


@pytest.mark.tier2
@pytest.mark.integration
@pytest.mark.cache
@pytest.mark.slow
def test_cache_promotion_race_and_cursor_fail_closed(
    client: AttuneClient, api_base_url: str, test_timeout: int, pack_ref: str
):
    with cache_namespace(client, owner_ref=pack_ref, prefix="race") as namespace:
        base_generation, base_refresh = publish_generation(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            entries=cache_entries(8, prefix="race", revision="base"),
            expected_active_generation_id=None,
            chunk_size=4,
        )
        assertion = assertion_id(namespace, base_refresh)
        candidates: list[int] = []
        for label in ("left", "right"):
            refresh_id = cache_refresh_ref(label)
            candidate = generation_id(
                client.cache_create_generation(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    client_refresh_id=refresh_id,
                    expected_active_generation_id=base_generation,
                    expected_chunk_count=1,
                    expected_record_count=3,
                )
            )
            client.cache_upload_chunk(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                generation_id=candidate,
                chunk_index=0,
                entries=cache_entries(3, prefix=label, revision=label),
            )
            client.cache_seal_generation(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                generation_id=candidate,
                expected_chunk_count=1,
                expected_record_count=3,
            )
            candidates.append(candidate)

        second_client = AttuneClient(base_url=api_base_url, timeout=test_timeout)

        def promote(actor: AttuneClient, candidate: int) -> tuple[int, int]:
            try:
                response = actor.cache_promote_generation(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=namespace,
                    generation_id=candidate,
                    expected_active_generation_id=base_generation,
                )
                return candidate, 200 if generation_id(response) == candidate else 500
            except CacheApiError as error:
                return candidate, response_status(error)

        with ThreadPoolExecutor(max_workers=2) as pool:
            outcomes = list(
                pool.map(
                    lambda args: promote(*args),
                    ((client, candidates[0]), (second_client, candidates[1])),
                )
            )
        second_client.logout()
        winners = [candidate for candidate, status in outcomes if status == 200]
        losers = [status for _, status in outcomes if status != 200]
        assert len(winners) == 1, f"optimistic promotion race {assertion}: {outcomes}"
        assert len(losers) == 1 and losers[0] in {400, 409, 412}, (
            f"losing promotion must fail precondition {assertion}: {outcomes}"
        )

        first_page = client.cache_scan(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=pack_ref,
            namespace=namespace,
            page_size=1,
        )
        cursor = first_page.get("next_cursor")
        winner = winners[0]
        assert cursor and generation_id(first_page) == winner, f"cursor setup {assertion}"
        assert_error_status(
            lambda: client.cache_scan(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                page_size=1,
                generation_id=winner,
                cursor="not-an-authenticated-cache-cursor",
            ),
            expected={400, 403, 404, 410, 422},
            assertion=f"malformed cursor {assertion}",
        )
        altered = f"{cursor[:-1]}{'A' if cursor[-1] != 'A' else 'B'}"
        assert_error_status(
            lambda: client.cache_scan(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=pack_ref,
                namespace=namespace,
                page_size=1,
                generation_id=winner,
                cursor=altered,
            ),
            expected={400, 403, 404, 410, 422},
            assertion=f"tampered cursor {assertion}",
        )

        with cache_namespace(client, owner_ref=pack_ref, prefix="cursor-other") as other:
            other_generation, other_refresh = publish_generation(
                client,
                owner_ref=pack_ref,
                namespace=other,
                entries=cache_entries(2, prefix="other"),
                expected_active_generation_id=None,
            )
            assert_error_status(
                lambda: client.cache_scan(
                    owner_type=CACHE_OWNER_TYPE,
                    owner_ref=pack_ref,
                    namespace=other,
                    page_size=1,
                    generation_id=other_generation,
                    cursor=cursor,
                ),
                expected={400, 403, 404, 410, 422},
                assertion=(
                    f"cross-namespace cursor "
                    f"{assertion_id(other, other_refresh)} source={namespace}"
                ),
            )

        wait_for_cursor_expiry(
            client,
            owner_ref=pack_ref,
            namespace=namespace,
            generation=winner,
            cursor=cursor,
        )
