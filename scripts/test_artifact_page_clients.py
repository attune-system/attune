"""Offline generated-client pagination contracts; uses the E2E venv's httpx/attrs."""

from pathlib import Path
import sys
import unittest

import httpx

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tests"))
from generated_client import AuthenticatedClient
from generated_client.api.artifacts import list_artifacts_by_execution, list_versions
from generated_client.models.artifact_type import ArtifactType
from generated_client.models.artifact_cursor_page_artifact_summary import (
    ArtifactCursorPageArtifactSummary,
)
from generated_client.models.artifact_cursor_page_artifact_version_summary import (
    ArtifactCursorPageArtifactVersionSummary,
)


class ArtifactPageClientTests(unittest.TestCase):
    def test_execution_page_and_progress_filter_use_the_typed_cursor_contract(self):
        def respond(request):
            self.assertEqual(request.url.path, "/api/v1/executions/42/artifacts")
            self.assertEqual(
                dict(request.url.params),
                {"per_page": "1", "type": "progress"},
            )
            return httpx.Response(
                200,
                json={
                    "items": [
                        {
                            "id": 17,
                            "ref": "fixture.progress",
                            "scope": "action",
                            "owner": "fixture.run",
                            "type": "progress",
                            "visibility": "public",
                            "classification": "general",
                            "created": "2026-10-14T00:00:00Z",
                            "updated": "2026-10-14T00:00:00Z",
                            "selected_version": {
                                "id": 91, "version": 3, "execution": 42,
                                "created": "2026-10-14T00:00:00Z",
                            },
                        }
                    ],
                    "next_cursor": "opaque-next",
                    "has_more": True,
                },
            )

        with AuthenticatedClient(
            base_url="http://artifact-contract.invalid",
            token="fixture-token",
            httpx_args={"transport": httpx.MockTransport(respond)},
        ) as client:
            result = list_artifacts_by_execution.sync(
                42, client=client, per_page=1, type_=ArtifactType.PROGRESS
            )
        self.assertIsInstance(result, ArtifactCursorPageArtifactSummary)
        self.assertEqual(result.items[0].id, 17)
        self.assertEqual(result.items[0].selected_version.execution, 42)
        self.assertEqual(result.items[0].selected_version.id, 91)
        self.assertEqual(result.next_cursor, "opaque-next")
        self.assertTrue(result.has_more)
        self.assertNotIn("pagination", result.to_dict())

    def test_history_page_retains_navigation_metadata(self):
        def respond(request):
            self.assertEqual(request.url.path, "/api/v1/artifacts/17/versions")
            self.assertEqual(dict(request.url.params), {"cursor": "opaque+/=", "per_page": "100"})
            return httpx.Response(
                200,
                json={
                    "items": [
                        {"id": 3, "version": 1, "created": "2026-10-14T00:00:00Z"}
                    ],
                    "next_cursor": None,
                    "has_more": False,
                },
            )

        with AuthenticatedClient(
            base_url="http://artifact-contract.invalid",
            token="fixture-token",
            httpx_args={"transport": httpx.MockTransport(respond)},
        ) as client:
            result = list_versions.sync(17, client=client, cursor="opaque+/=", per_page=100)
        self.assertIsInstance(result, ArtifactCursorPageArtifactVersionSummary)
        self.assertEqual(result.items[0].version, 1)
        self.assertIsNone(result.next_cursor)
        self.assertFalse(result.has_more)

    def test_invalid_pages_do_not_parse_as_successful_empty_histories(self):
        with AuthenticatedClient(
            base_url="http://artifact-contract.invalid",
            token="fixture-token",
            httpx_args={
                "transport": httpx.MockTransport(lambda _: httpx.Response(400))
            },
        ) as client:
            response = list_versions.sync_detailed(17, client=client, per_page=0)
        self.assertEqual(response.status_code, 400)
        self.assertIsNone(response.parsed)

    def test_unbounded_data_arrays_are_not_an_alternate_v1_reader(self):
        for model in [
            ArtifactCursorPageArtifactSummary,
            ArtifactCursorPageArtifactVersionSummary,
        ]:
            with self.assertRaises(KeyError):
                model.from_dict({"data": []})


if __name__ == "__main__":
    unittest.main()
