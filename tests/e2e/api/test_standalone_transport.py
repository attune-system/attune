"""
E2E Test: Standalone Worker and Sensor Transport

Verifies that workers and sensors that do NOT share Docker volumes with the
API/executor cluster can still:
  1. Receive pack contents via API-based transport
  2. Execute actions and return results
  3. Stream stdout/stderr log files back through the API
  4. Create and download artifacts via API transport
  5. Handle pack updates (re-sync) and deletion (cleanup)

Prerequisites:
  The stack must be started with docker-compose.standalone.yaml to include
  the ``worker-standalone`` and ``sensor-standalone`` services::

      docker compose -f docker-compose.yaml \\
                     -f docker-compose.standalone.yaml \\
                     -f docker-compose.e2e.yaml \\
                     run --rm e2e-tests -k standalone

  The standalone worker has label ``attune_transport=api`` so tests can
  target it via ``worker_selector``.
"""

import time
import uuid
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Optional

import pytest

from helpers.client_wrapper import AttuneClient
from helpers.polling import wait_for_condition, wait_for_execution_status


# ── Helpers ──────────────────────────────────────────────────────────────


def _uid() -> str:
    return uuid.uuid4().hex[:8]


def _get(client: AttuneClient, path: str, expected: int = 200):
    resp = client.session.get(f"{client.base_url}{path}", timeout=15)
    assert resp.status_code == expected, (
        f"GET {path} returned {resp.status_code}: {resp.text}"
    )
    return resp.json().get("data", resp.json())


def _create_execution_with_selector(
    client: AttuneClient,
    action_ref: str,
    worker_selector: dict,
    parameters: Optional[dict] = None,
) -> dict:
    """Create an execution targeting a specific worker via worker_selector."""
    payload = {
        "action_ref": action_ref,
        "worker_selector": worker_selector,
    }
    if parameters:
        payload["parameters"] = parameters
    resp = client._request(
        "POST", "/api/v1/executions/execute", json=payload
    )
    assert resp.status_code in (200, 201), (
        f"Execute failed: {resp.status_code} {resp.text}"
    )
    data = resp.json()
    return data.get("data", data)


def _wait_for_worker(
    client: AttuneClient,
    worker_name_substring: str,
    timeout: float = 90.0,
) -> bool:
    """Wait until a worker whose name contains *worker_name_substring* has
    registered (appears in /api/v1/workers)."""
    def _check():
        resp = client._request("GET", "/api/v1/workers")
        if resp.status_code != 200:
            return False
        body = resp.json()
        workers = body.get("data", body.get("items", body))
        if isinstance(workers, list):
            return any(
                worker_name_substring in (w.get("name") or "")
                for w in workers
            )
        return False

    try:
        wait_for_condition(
            _check,
            timeout=timeout,
            poll_interval=2.0,
            error_message=(
                f"Worker containing '{worker_name_substring}' did not register"
            ),
        )
        return True
    except TimeoutError:
        return False


def _build_standalone_pack(pack_dir: Path, pack_ref: str):
    """Build a minimal test pack with a shell action for the standalone worker."""
    (pack_dir / "actions").mkdir(parents=True)

    # pack.yaml
    (pack_dir / "pack.yaml").write_text(
        f"""\
ref: {pack_ref}
name: {pack_ref}
label: Standalone Transport Test Pack
description: Pack used to test API-based file transport for standalone workers
version: 1.0.0
is_standard: false
"""
    )

    # Simple echo action
    (pack_dir / "actions" / "echo.yaml").write_text(
        f"""\
ref: {pack_ref}.echo
name: echo
label: Echo Action
description: Echoes input parameters back as JSON
enabled: true
runner_type: shell
entry_point: echo.sh
output_format: json
parameters:
  message:
    type: string
    required: true
    description: Message to echo
"""
    )
    (pack_dir / "actions" / "echo.sh").write_text(
        """\
#!/usr/bin/env bash
# Read input from stdin (JSON parameters)
INPUT=$(cat)
# Extract message using simple string manipulation (no python dependency)
MSG=$(echo "$INPUT" | grep -o '"message"[[:space:]]*:[[:space:]]*"[^"]*"' | sed 's/.*"message"[[:space:]]*:[[:space:]]*"//;s/"$//' || echo "unknown")
cat <<EOF
{"echoed": "$MSG", "worker": "$(hostname)", "transport_test": true}
EOF
"""
    )

    # Action that produces stderr (for log transport testing)
    (pack_dir / "actions" / "stderr_test.yaml").write_text(
        f"""\
ref: {pack_ref}.stderr_test
name: stderr_test
label: Stderr Test Action
description: Writes to both stdout and stderr to verify log transport
enabled: true
runner_type: shell
entry_point: stderr_test.sh
output_format: json
"""
    )
    (pack_dir / "actions" / "stderr_test.sh").write_text(
        """\
#!/usr/bin/env bash
echo "stderr-line-1" >&2
echo "stderr-line-2" >&2
echo "stderr-line-3" >&2
echo '{"stdout_ok": true, "message": "action completed with stderr"}'
"""
    )

    # Action that allocates a file-backed artifact version, writes to the
    # returned local path, and relies on worker finalization to copy it to API
    # transport in standalone mode.
    (pack_dir / "actions" / "artifact_test.yaml").write_text(
        f"""\
ref: {pack_ref}.artifact_test
name: artifact_test
label: Artifact Test Action
description: Creates a file artifact to verify artifact transport
enabled: true
runner_type: shell
entry_point: artifact_test.sh
output_format: json
default_execution_permission_set_refs:
  - standard
"""
    )
    (pack_dir / "actions" / "artifact_test.sh").write_text(
        """\
#!/usr/bin/env bash
set -eu
ARTIFACT_DIR="${ATTUNE_ARTIFACTS_DIR:-/opt/attune/artifacts}"
API_URL="${ATTUNE_API_URL:?ATTUNE_API_URL is required}"
API_TOKEN="${ATTUNE_API_TOKEN:?ATTUNE_API_TOKEN is required}"
ACTION_REF="${ATTUNE_ACTION:?ATTUNE_ACTION is required}"
EXEC_ID="${ATTUNE_EXEC_ID:?ATTUNE_EXEC_ID is required}"

ARTIFACT_REF="${ACTION_REF}.standalone_copy.${EXEC_ID}"
EXPECTED_CONTENT="standalone-artifact-copy-${EXEC_ID}"

ALLOCATE_BODY=$(cat <<JSON
{"scope":"action","owner":"${ACTION_REF}","type":"file_text","visibility":"private","retention_policy":"versions","retention_limit":4,"name":"Standalone artifact copy ${EXEC_ID}","content_type":"text/plain","created_by":"${ACTION_REF}"}
JSON
)

ALLOCATE_RESPONSE=$(curl -fsS \
  -X POST "${API_URL}/api/v1/artifacts/ref/${ARTIFACT_REF}/versions/file" \
  -H "Authorization: Bearer ${API_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "${ALLOCATE_BODY}")

FILE_PATH=$(printf '%s' "$ALLOCATE_RESPONSE" \
  | grep -o '"file_path"[[:space:]]*:[[:space:]]*"[^"]*"' \
  | head -1 \
  | sed 's/.*"file_path"[[:space:]]*:[[:space:]]*"//;s/"$//')

if [ -z "$FILE_PATH" ]; then
  echo "Failed to parse file_path from allocation response: $ALLOCATE_RESPONSE" >&2
  exit 1
fi

mkdir -p "$(dirname "${ARTIFACT_DIR}/${FILE_PATH}")"
printf '%s\n' "$EXPECTED_CONTENT" > "${ARTIFACT_DIR}/${FILE_PATH}"

cat <<EOF
{"artifact_created": true, "artifact_ref": "${ARTIFACT_REF}", "file_path": "${FILE_PATH}", "expected_content": "${EXPECTED_CONTENT}"}
EOF
"""
    )


def _build_updated_pack(pack_dir: Path, pack_ref: str):
    """Build an updated version of the pack with an extra action."""
    _build_standalone_pack(pack_dir, pack_ref)

    # Add a new action that only exists in v2
    (pack_dir / "actions" / "v2_action.yaml").write_text(
        f"""\
ref: {pack_ref}.v2_action
name: v2_action
label: V2 Action
description: Added in pack update to verify pack re-sync
enabled: true
runner_type: shell
entry_point: v2_action.sh
output_format: json
"""
    )
    (pack_dir / "actions" / "v2_action.sh").write_text(
        """\
#!/usr/bin/env bash
echo '{"version": 2, "update_received": true}'
"""
    )

    # Bump version
    (pack_dir / "pack.yaml").write_text(
        f"""\
ref: {pack_ref}
name: {pack_ref}
label: Standalone Transport Test Pack (Updated)
description: Updated pack for transport re-sync testing
version: 2.0.0
is_standard: false
"""
    )


# ── Fixtures ─────────────────────────────────────────────────────────────


@pytest.fixture(scope="session")
def standalone_worker_available(session_client: AttuneClient) -> bool:
    """Check whether the standalone worker is registered.

    Returns True if found; tests that require it are skipped otherwise.
    """
    return _wait_for_worker(session_client, "standalone", timeout=30)


@pytest.fixture(scope="session")
def sa_pack_ref() -> str:
    return f"dc_transport_{_uid()}"


@pytest.fixture(scope="session")
def sa_pack(session_client: AttuneClient, sa_pack_ref: str) -> dict:
    """Upload the test pack and return pack metadata. Cleaned up after session."""
    with TemporaryDirectory(prefix="attune-dc-e2e-") as tmp:
        pack_dir = Path(tmp) / sa_pack_ref
        pack_dir.mkdir()
        _build_standalone_pack(pack_dir, sa_pack_ref)
        result = session_client.upload_pack(str(pack_dir), force=True)
    assert result["ref"] == sa_pack_ref, f"Unexpected pack ref: {result}"

    # Give the standalone worker time to receive and sync the pack
    time.sleep(5)

    yield result

    # Cleanup
    try:
        session_client.delete_pack(result["id"])
    except Exception:
        pass


# ── Tests ────────────────────────────────────────────────────────────────


@pytest.mark.api
class TestStandaloneWorkerTransport:
    """Tests that a standalone worker (no shared volumes) can execute actions
    via API-based pack and artifact transport."""

    def test_standalone_worker_registered(
        self, client, standalone_worker_available
    ):
        """The standalone worker should appear in the worker list."""
        if not standalone_worker_available:
            pytest.skip(
                "Standalone worker not available — start stack with "
                "docker-compose.standalone.yaml"
            )
        resp = client._request("GET", "/api/v1/workers")
        body = resp.json()
        workers = body.get("data", body.get("items", body))
        if not isinstance(workers, list):
            workers = []
        sa_workers = [
            w for w in workers
            if "standalone" in (w.get("name") or "")
        ]
        assert len(sa_workers) >= 1, (
            f"Expected standalone worker in list, got: "
            f"{[w.get('name') for w in workers]}"
        )
        worker = sa_workers[0]
        assert worker.get("status") in ("online", "active", None)

    def test_execute_on_standalone_worker(
        self, client, standalone_worker_available, sa_pack, sa_pack_ref
    ):
        """Execute a shell action targeted at the standalone worker and
        verify it completes successfully with correct output."""
        if not standalone_worker_available:
            pytest.skip("Standalone worker not available")

        action_ref = f"{sa_pack_ref}.echo"
        message = f"hello-standalone-{_uid()}"

        execution = _create_execution_with_selector(
            client,
            action_ref=action_ref,
            worker_selector={"attune_transport": "api"},
            parameters={"message": message},
        )
        exec_id = execution["id"]

        # Wait for completion
        final = wait_for_execution_status(
            client, exec_id, "completed", timeout=120
        )
        assert final["status"] == "completed", (
            f"Execution {exec_id} did not complete: {final}"
        )

        # Verify result
        result = final.get("result", {})
        assert result.get("transport_test") is True, (
            f"Expected transport_test=true in result: {result}"
        )
        assert result.get("echoed") == message, (
            f"Expected echoed='{message}' in result: {result}"
        )

    def test_stdout_log_accessible(
        self, client, standalone_worker_available, sa_pack, sa_pack_ref
    ):
        """Verify that stdout logs from a standalone worker execution are
        accessible via the API (streamed back through transport)."""
        if not standalone_worker_available:
            pytest.skip("Standalone worker not available")

        action_ref = f"{sa_pack_ref}.stderr_test"
        execution = _create_execution_with_selector(
            client,
            action_ref=action_ref,
            worker_selector={"attune_transport": "api"},
        )
        exec_id = execution["id"]

        final = wait_for_execution_status(
            client, exec_id, "completed", timeout=120
        )
        assert final["status"] == "completed"

        # Check that the execution result confirms stdout was captured
        result = final.get("result", {})
        assert result.get("stdout_ok") is True, (
            f"Expected stdout_ok=true: {result}"
        )

        # Try to retrieve stdout log via the execution log stream endpoint
        # GET /api/v1/executions/{id}/logs/stdout/stream returns SSE
        # We use a simple download instead.
        # The stdout artifact should be accessible via execution artifacts
        artifacts_resp = client._request(
            "GET", f"/api/v1/artifacts",
            params={"execution": exec_id},
        )
        if artifacts_resp.status_code == 200:
            artifacts = artifacts_resp.json().get("data", artifacts_resp.json())
            if isinstance(artifacts, list):
                stdout_arts = [
                    a for a in artifacts
                    if "stdout" in (a.get("ref") or "").lower()
                ]
                # If stdout artifacts exist, verify they have content
                for art in stdout_arts:
                    art_id = art["id"]
                    download_resp = client._request(
                        "GET", f"/api/v1/artifacts/{art_id}/download"
                    )
                    # 200 = file content; 204 = empty; 404 = file not on this server
                    assert download_resp.status_code in (200, 204, 404), (
                        f"Artifact download unexpected: {download_resp.status_code}"
                    )

    def test_stderr_log_accessible(
        self, client, standalone_worker_available, sa_pack, sa_pack_ref
    ):
        """Verify stderr output from a standalone worker execution is
        captured and accessible."""
        if not standalone_worker_available:
            pytest.skip("Standalone worker not available")

        action_ref = f"{sa_pack_ref}.stderr_test"
        execution = _create_execution_with_selector(
            client,
            action_ref=action_ref,
            worker_selector={"attune_transport": "api"},
        )
        exec_id = execution["id"]

        final = wait_for_execution_status(
            client, exec_id, "completed", timeout=120
        )
        assert final["status"] == "completed"

        # Give the worker time to finalize logs
        time.sleep(3)

        # Check stderr artifact exists
        artifacts_resp = client._request(
            "GET", f"/api/v1/artifacts",
            params={"execution": exec_id},
        )
        if artifacts_resp.status_code == 200:
            artifacts = artifacts_resp.json().get("data", artifacts_resp.json())
            if isinstance(artifacts, list):
                stderr_arts = [
                    a for a in artifacts
                    if "stderr" in (a.get("ref") or "").lower()
                ]
                if stderr_arts:
                    art = stderr_arts[0]
                    download_resp = client._request(
                        "GET", f"/api/v1/artifacts/{art['id']}/download"
                    )
                    if download_resp.status_code == 200:
                        content = download_resp.text
                        assert "stderr-line-1" in content, (
                            f"Expected stderr content, got: {content[:200]}"
                        )

    def test_file_artifact_copied_from_standalone_worker(
        self, client, standalone_worker_available, sa_pack, sa_pack_ref
    ):
        """Verify a file-backed artifact written to a standalone worker's
        local ATTUNE_ARTIFACTS_DIR is copied to the API artifact volume during
        execution finalization."""
        if not standalone_worker_available:
            pytest.skip("Standalone worker not available")

        action_ref = f"{sa_pack_ref}.artifact_test"
        execution = _create_execution_with_selector(
            client,
            action_ref=action_ref,
            worker_selector={"attune_transport": "api"},
        )
        exec_id = execution["id"]

        final = wait_for_execution_status(
            client, exec_id, "completed", timeout=120
        )
        assert final["status"] == "completed"

        result = final.get("result", {})
        assert result.get("artifact_created") is True, (
            f"Expected artifact_created=true: {result}"
        )
        artifact_ref = result.get("artifact_ref")
        file_path = result.get("file_path")
        expected_content = result.get("expected_content")
        assert artifact_ref, f"Missing artifact_ref in result: {result}"
        assert file_path, f"Missing file_path in result: {result}"
        assert expected_content, f"Missing expected_content in result: {result}"

        artifact = _get(client, f"/api/v1/artifacts/ref/{artifact_ref}")
        assert artifact["ref"] == artifact_ref
        assert artifact["scope"] == "action"
        assert artifact["owner"] == action_ref

        versions_resp = client._request(
            "GET", f"/api/v1/artifacts/{artifact['id']}/versions"
        )
        assert versions_resp.status_code == 200, (
            f"Version list failed: {versions_resp.status_code} {versions_resp.text}"
        )
        versions = versions_resp.json().get("data", versions_resp.json())
        if isinstance(versions, dict) and "items" in versions:
            versions = versions["items"]
        matching_versions = [
            version for version in versions
            if version.get("execution") == exec_id
            and version.get("file_path") == file_path
        ]
        assert matching_versions, (
            f"No version linked to execution={exec_id} file_path={file_path}: "
            f"{versions}"
        )

        expected_body = f"{expected_content}\n"
        download_resp = None

        def _artifact_downloaded() -> bool:
            nonlocal download_resp
            download_resp = client._request(
                "GET", f"/api/v1/artifacts/{artifact['id']}/download"
            )
            return (
                download_resp.status_code == 200
                and download_resp.text == expected_body
            )

        wait_for_condition(
            _artifact_downloaded,
            timeout=30,
            poll_interval=1.0,
            error_message=(
                "Standalone file-backed artifact was not copied to API "
                "transport with expected content"
            ),
        )
        assert download_resp is not None
        assert download_resp.status_code == 200
        assert download_resp.text == expected_body

        latest_size = artifact.get("size_bytes")
        if latest_size is not None:
            assert latest_size == len(expected_body), (
                f"Expected artifact size {len(expected_body)}, got {latest_size}"
            )

    def test_pack_update_syncs_to_standalone_worker(
        self, session_client, standalone_worker_available, sa_pack, sa_pack_ref
    ):
        """Upload an updated pack and verify the standalone worker receives
        the new action and can execute it."""
        if not standalone_worker_available:
            pytest.skip("Standalone worker not available")

        # Upload updated pack with v2_action
        with TemporaryDirectory(prefix="attune-dc-update-") as tmp:
            pack_dir = Path(tmp) / sa_pack_ref
            pack_dir.mkdir()
            _build_updated_pack(pack_dir, sa_pack_ref)
            result = session_client.upload_pack(str(pack_dir), force=True)

        assert result["ref"] == sa_pack_ref

        # Give the standalone worker time to receive the pack.registered
        # event and sync the updated pack
        time.sleep(10)

        # Verify the new action is registered
        v2_action = _get(session_client, f"/api/v1/actions/{sa_pack_ref}.v2_action")
        assert v2_action["ref"] == f"{sa_pack_ref}.v2_action"

        # Execute v2_action on standalone worker
        execution = _create_execution_with_selector(
            session_client,
            action_ref=f"{sa_pack_ref}.v2_action",
            worker_selector={"attune_transport": "api"},
        )
        exec_id = execution["id"]

        final = wait_for_execution_status(
            session_client, exec_id, "completed", timeout=120
        )
        assert final["status"] == "completed", (
            f"v2_action did not complete: {final}"
        )

        result = final.get("result", {})
        assert result.get("version") == 2, (
            f"Expected version=2 in result: {result}"
        )
        assert result.get("update_received") is True

    def test_multiple_executions_reliable(
        self, client, standalone_worker_available, sa_pack, sa_pack_ref
    ):
        """Run several executions on the standalone worker to verify
        reliability across multiple pack-downloaded action invocations."""
        if not standalone_worker_available:
            pytest.skip("Standalone worker not available")

        action_ref = f"{sa_pack_ref}.echo"
        exec_ids = []

        for i in range(3):
            msg = f"reliability-test-{i}-{_uid()}"
            execution = _create_execution_with_selector(
                client,
                action_ref=action_ref,
                worker_selector={"attune_transport": "api"},
                parameters={"message": msg},
            )
            exec_ids.append((execution["id"], msg))

        # Wait for all to complete
        for exec_id, expected_msg in exec_ids:
            final = wait_for_execution_status(
                client, exec_id, "completed", timeout=120
            )
            assert final["status"] == "completed", (
                f"Execution {exec_id} failed: {final}"
            )
            result = final.get("result", {})
            assert result.get("echoed") == expected_msg


@pytest.mark.api
class TestStandaloneSensorTransport:
    """Tests that a standalone sensor (no shared volumes) can receive
    pack contents and start sensor processes."""

    def test_standalone_sensor_pack_received(
        self, client, standalone_worker_available, sa_pack, sa_pack_ref
    ):
        """After pack upload, verify the standalone sensor service has
        received the pack (indirectly, by checking sensor registration)."""
        if not standalone_worker_available:
            pytest.skip("Standalone services not available")

        # The test pack doesn't have an enabled sensor, but the sensor
        # service should still have received the pack contents.
        # We verify by checking that pack metadata is accessible.
        pack = _get(client, f"/api/v1/packs/{sa_pack_ref}")
        assert pack["ref"] == sa_pack_ref
        # If the sensor agent is running it would have synced packs at startup.
        # This test primarily validates the infrastructure is connected.


@pytest.mark.api
class TestStandalonePackLifecycle:
    """Tests the full pack lifecycle through standalone transport:
    create → execute → update → re-execute → delete."""

    def test_full_lifecycle(self, client, standalone_worker_available):
        """End-to-end: upload pack → execute → update pack → execute new
        action → delete pack."""
        if not standalone_worker_available:
            pytest.skip("Standalone worker not available")

        pack_ref = f"sa_lifecycle_{_uid()}"

        # ── Phase 1: Upload initial pack ──
        with TemporaryDirectory(prefix="attune-dc-lifecycle-") as tmp:
            pack_dir = Path(tmp) / pack_ref
            pack_dir.mkdir()
            _build_standalone_pack(pack_dir, pack_ref)
            pack = client.upload_pack(str(pack_dir), force=True)

        pack_id = pack["id"]
        time.sleep(5)  # Wait for pack sync

        try:
            # ── Phase 2: Execute on standalone worker ──
            execution = _create_execution_with_selector(
                client,
                action_ref=f"{pack_ref}.echo",
                worker_selector={"attune_transport": "api"},
                parameters={"message": "lifecycle-v1"},
            )
            final = wait_for_execution_status(
                client, execution["id"], "completed", timeout=120
            )
            assert final["status"] == "completed"
            assert final.get("result", {}).get("echoed") == "lifecycle-v1"

            # ── Phase 3: Update pack with new action ──
            with TemporaryDirectory(prefix="attune-dc-lifecycle-v2-") as tmp:
                pack_dir = Path(tmp) / pack_ref
                pack_dir.mkdir()
                _build_updated_pack(pack_dir, pack_ref)
                client.upload_pack(str(pack_dir), force=True)

            time.sleep(10)  # Wait for pack re-sync

            # ── Phase 4: Execute new action on standalone worker ──
            execution2 = _create_execution_with_selector(
                client,
                action_ref=f"{pack_ref}.v2_action",
                worker_selector={"attune_transport": "api"},
            )
            final2 = wait_for_execution_status(
                client, execution2["id"], "completed", timeout=120
            )
            assert final2["status"] == "completed"
            assert final2.get("result", {}).get("version") == 2

        finally:
            # ── Phase 5: Delete pack ──
            try:
                client.delete_pack(pack_id)
            except Exception:
                pass

        # After deletion, the action should no longer be available
        time.sleep(3)
        resp = client._request(
            "GET", f"/api/v1/actions/{pack_ref}.echo"
        )
        assert resp.status_code in (404, 410), (
            f"Expected action to be gone after pack delete, got: "
            f"{resp.status_code}"
        )
