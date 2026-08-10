"""Public-API helpers shared by cache end-to-end scenarios."""

from __future__ import annotations

import os
import time
import uuid
from contextlib import contextmanager
from datetime import datetime, timezone
from typing import Iterator, Sequence

from helpers import AttuneClient, wait_for_condition
from helpers.client_wrapper import CacheApiError


CACHE_OWNER_TYPE = "pack"
CACHE_CLEANUP_TIMEOUT_SECONDS = float(
    os.getenv("ATTUNE_E2E_CACHE_CLEANUP_TIMEOUT_SECONDS", "30")
)


def assertion_id(namespace: str, refresh_id: str) -> str:
    return f"namespace={namespace} refresh={refresh_id}"


def cache_namespace_ref(prefix: str) -> str:
    """Return an API-valid, test-unique namespace without sharing global state."""
    return f"{prefix}-{uuid.uuid4().hex[:20]}".lower()


def cache_refresh_ref(prefix: str) -> str:
    return f"{prefix}-{uuid.uuid4().hex}"


def high_entropy_sentinel() -> str:
    return f"cache-sentinel-{uuid.uuid4().hex}-{uuid.uuid4().hex}"


def generation_id(response: dict) -> int:
    value = response.get("generation_id", response.get("id"))
    if value is None and isinstance(response.get("generation"), dict):
        value = response["generation"].get("id")
    assert isinstance(value, int), f"Cache response has no generation id: {response}"
    return value


def active_generation_id(response: dict) -> int | None:
    value = response.get("active_generation_id", response.get("active_generation"))
    if isinstance(value, dict):
        value = value.get("id")
    return value if isinstance(value, int) else None


def cache_entries(
    count: int,
    *,
    prefix: str = "record",
    sentinel: str | None = None,
    revision: str = "v1",
) -> list[dict]:
    """Build compact deterministic test records with an optional safe sentinel."""
    return [
        {
            "external_id": f"{prefix}-{index:06d}",
            "value": {
                "ordinal": index,
                "revision": revision,
                **({"sentinel": sentinel} if sentinel else {}),
            },
        }
        for index in range(count)
    ]


def scan_all(
    client: AttuneClient,
    *,
    owner_ref: str,
    namespace: str,
    page_size: int,
) -> tuple[int, list[dict], str | None]:
    """Traverse one public cache snapshot, retaining the pinned generation."""
    generation: int | None = None
    cursor: str | None = None
    cursor_expires_at: str | None = None
    entries: list[dict] = []

    while True:
        page = client.cache_scan(
            owner_type=CACHE_OWNER_TYPE,
            owner_ref=owner_ref,
            namespace=namespace,
            page_size=page_size,
            generation_id=generation,
            cursor=cursor,
        )
        current_generation = generation_id(page)
        if generation is None:
            generation = current_generation
        assert current_generation == generation, (
            f"generation changed while scanning namespace={namespace}: "
            f"expected={generation}, received={current_generation}"
        )
        entries.extend(page.get("items", page.get("entries", [])))
        cursor = page.get("next_cursor")
        cursor_expires_at = page.get("cursor_expires_at", cursor_expires_at)
        if not cursor:
            break

    assert generation is not None, f"Empty scan omitted generation for namespace={namespace}"
    return generation, entries, cursor_expires_at


def publish_generation(
    client: AttuneClient,
    *,
    owner_ref: str,
    namespace: str,
    entries: Sequence[dict],
    expected_active_generation_id: int | None,
    chunk_size: int = 20,
    refresh_id: str | None = None,
    owner_type: str = CACHE_OWNER_TYPE,
) -> tuple[int, str]:
    """Exercise create → numbered chunks → seal → optimistic promotion."""
    assert entries, "Use this helper for non-empty cache generations only"
    refresh_id = refresh_id or cache_refresh_ref("refresh")
    chunk_count = (len(entries) + chunk_size - 1) // chunk_size
    created = client.cache_create_generation(
        owner_type=owner_type,
        owner_ref=owner_ref,
        namespace=namespace,
        client_refresh_id=refresh_id,
        expected_active_generation_id=expected_active_generation_id,
        expected_chunk_count=chunk_count,
        expected_record_count=len(entries),
    )
    created_generation = generation_id(created)

    for chunk_index in range(chunk_count):
        start = chunk_index * chunk_size
        client.cache_upload_chunk(
            owner_type=owner_type,
            owner_ref=owner_ref,
            namespace=namespace,
            generation_id=created_generation,
            chunk_index=chunk_index,
            entries=list(entries[start : start + chunk_size]),
        )

    sealed = client.cache_seal_generation(
        owner_type=owner_type,
        owner_ref=owner_ref,
        namespace=namespace,
        generation_id=created_generation,
        expected_chunk_count=chunk_count,
        expected_record_count=len(entries),
    )
    assert generation_id(sealed) == created_generation, (
        f"Seal changed generation for {assertion_id(namespace, refresh_id)}"
    )
    promoted = client.cache_promote_generation(
        owner_type=owner_type,
        owner_ref=owner_ref,
        namespace=namespace,
        generation_id=created_generation,
        expected_active_generation_id=expected_active_generation_id,
    )
    assert generation_id(promoted) == created_generation, (
        f"Promotion changed generation for {assertion_id(namespace, refresh_id)}"
    )
    return created_generation, refresh_id


@contextmanager
def cache_namespace(
    client: AttuneClient,
    *,
    owner_ref: str,
    prefix: str,
    policy: dict | None = None,
    namespace: str | None = None,
    owner_type: str = CACHE_OWNER_TYPE,
) -> Iterator[str]:
    """Create and lifecycle-delete one isolated namespace using only public APIs."""
    namespace = namespace or cache_namespace_ref(prefix)
    client.cache_create_namespace(
        owner_type=owner_type,
        owner_ref=owner_ref,
        namespace=namespace,
        policy=policy,
    )
    try:
        yield namespace
    finally:
        try:
            client.cache_delete_namespace(
                owner_type=owner_type,
                owner_ref=owner_ref,
                namespace=namespace,
            )
        except CacheApiError as error:
            if error.response.status_code not in (404, 410):
                raise

        def deleted() -> bool:
            try:
                client.cache_get_namespace(
                    owner_type=owner_type,
                    owner_ref=owner_ref,
                    namespace=namespace,
                )
            except CacheApiError as error:
                return error.response.status_code in (404, 410)
            return False

        wait_for_condition(
            deleted,
            timeout=CACHE_CLEANUP_TIMEOUT_SECONDS,
            error_message=f"Cache namespace teardown did not complete: {namespace}",
        )


def response_status(error: CacheApiError) -> int:
    return error.response.status_code


def assert_error_status(
    operation,
    *,
    expected: set[int],
    assertion: str,
) -> CacheApiError:
    try:
        operation()
    except CacheApiError as error:
        assert response_status(error) in expected, (
            f"{assertion}: expected HTTP {sorted(expected)}, "
            f"received {response_status(error)} body={error.body}"
        )
        return error
    raise AssertionError(f"{assertion}: cache API unexpectedly succeeded")


def wait_for_cursor_expiry(
    client: AttuneClient,
    *,
    owner_ref: str,
    namespace: str,
    generation: int,
    cursor: str,
) -> None:
    """Poll a server-configured cursor lifetime instead of imposing a sleep."""

    def cursor_expired() -> bool:
        try:
            client.cache_scan(
                owner_type=CACHE_OWNER_TYPE,
                owner_ref=owner_ref,
                namespace=namespace,
                page_size=1,
                generation_id=generation,
                cursor=cursor,
            )
        except CacheApiError as error:
            body = str(error.body).lower()
            return response_status(error) == 410 or "snapshot_expired" in body
        return False

    wait_for_condition(
        cursor_expired,
        timeout=float(os.getenv("ATTUNE_E2E_CACHE_CURSOR_EXPIRY_TIMEOUT_SECONDS", "45")),
        error_message=(
            "Cache cursor did not expire within the E2E profile window "
            f"for namespace={namespace} generation={generation}"
        ),
    )


def create_metrics_artifact(
    client: AttuneClient,
    *,
    owner_ref: str,
    namespace: str,
    metrics: dict,
) -> dict:
    """Persist load-test metrics through the public artifact API."""
    response = client.post(
        "/api/v1/artifacts",
        json={
            "ref": f"{owner_ref}.cache-load-{namespace}",
            "scope": "pack",
            "owner": owner_ref,
            "type": "progress",
            "visibility": "private",
            "name": f"Cache load metrics: {namespace}",
            "content_type": "application/json",
            "data": {
                "captured_at": datetime.now(timezone.utc).isoformat(),
                **metrics,
            },
        },
    )
    assert response.status_code == 201, (
        f"Cache load metrics artifact failed for namespace={namespace}: "
        f"{response.status_code} {response.text}"
    )
    body = response.json()
    return body.get("data", body)


def cache_metric_lines(client: AttuneClient) -> list[str]:
    """Capture cache-specific service telemetry without container inspection."""
    response = client.get("/metrics")
    if response.status_code != 200:
        return [f"metrics_endpoint_status={response.status_code}"]
    return [
        line
        for line in response.text.splitlines()
        if "cache" in line.lower()
    ][:200]
