from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.artifact_version_by_ref_upload_form import ArtifactVersionByRefUploadForm
from ...models.upload_version_by_ref_response_201 import UploadVersionByRefResponse201
from ...types import Response


def _get_kwargs(
    ref: str,
    *,
    body: ArtifactVersionByRefUploadForm,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/artifacts/ref/{ref}/versions/upload".format(
            ref=quote(str(ref), safe=""),
        ),
    }

    _kwargs["files"] = body.to_multipart()

    headers["Content-Type"] = "multipart/form-data; boundary=+++"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | UploadVersionByRefResponse201 | None:
    if response.status_code == 201:
        response_201 = UploadVersionByRefResponse201.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 413:
        response_413 = cast(Any, None)
        return response_413

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | UploadVersionByRefResponse201]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionByRefUploadForm,
) -> Response[Any | UploadVersionByRefResponse201]:
    """Upload a file version to an artifact identified by ref, creating the artifact if it does not
    already exist.

     This is the recommended way for actions to produce versioned file artifacts. The caller
    provides the artifact ref and file content in a single multipart request. The server:

    1. Looks up the artifact by `ref`.
    2. If not found, creates it using the metadata fields in the multipart body.
    3. If found, optionally updates the `execution` link to the current execution.
    4. Uploads the file bytes as a new version (version number is auto-assigned).

    **Multipart fields:**
    - `file` (required) — the binary file content
    - `ref` (required for creation) — artifact reference (ignored if artifact already exists)
    - `scope` — owner scope: `system`, `pack`, `action`, `sensor`, `rule` (default: `action`)
    - `owner` — owner identifier (default: empty string)
    - `type` — artifact type: `file_text`, `file_image`, etc. (default: `file_text`)
    - `visibility` — `public` or `private` (default: type-aware server default)
    - `name` — human-readable name
    - `description` — optional description
    - `content_type` — MIME type (default: auto-detected from multipart or `application/octet-stream`)
    - `execution` — execution ID to link this artifact to (updates existing artifacts too)
    - `retention_policy` — `versions`, `days`, `hours`, `minutes` (default: `versions`)
    - `retention_limit` — limit value (default: `10`)
    - `created_by` — who created this version
    - `meta` — JSON metadata for this version

    Args:
        ref (str):
        body (ArtifactVersionByRefUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UploadVersionByRefResponse201]
    """

    kwargs = _get_kwargs(
        ref=ref,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionByRefUploadForm,
) -> Any | UploadVersionByRefResponse201 | None:
    """Upload a file version to an artifact identified by ref, creating the artifact if it does not
    already exist.

     This is the recommended way for actions to produce versioned file artifacts. The caller
    provides the artifact ref and file content in a single multipart request. The server:

    1. Looks up the artifact by `ref`.
    2. If not found, creates it using the metadata fields in the multipart body.
    3. If found, optionally updates the `execution` link to the current execution.
    4. Uploads the file bytes as a new version (version number is auto-assigned).

    **Multipart fields:**
    - `file` (required) — the binary file content
    - `ref` (required for creation) — artifact reference (ignored if artifact already exists)
    - `scope` — owner scope: `system`, `pack`, `action`, `sensor`, `rule` (default: `action`)
    - `owner` — owner identifier (default: empty string)
    - `type` — artifact type: `file_text`, `file_image`, etc. (default: `file_text`)
    - `visibility` — `public` or `private` (default: type-aware server default)
    - `name` — human-readable name
    - `description` — optional description
    - `content_type` — MIME type (default: auto-detected from multipart or `application/octet-stream`)
    - `execution` — execution ID to link this artifact to (updates existing artifacts too)
    - `retention_policy` — `versions`, `days`, `hours`, `minutes` (default: `versions`)
    - `retention_limit` — limit value (default: `10`)
    - `created_by` — who created this version
    - `meta` — JSON metadata for this version

    Args:
        ref (str):
        body (ArtifactVersionByRefUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UploadVersionByRefResponse201
    """

    return sync_detailed(
        ref=ref,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionByRefUploadForm,
) -> Response[Any | UploadVersionByRefResponse201]:
    """Upload a file version to an artifact identified by ref, creating the artifact if it does not
    already exist.

     This is the recommended way for actions to produce versioned file artifacts. The caller
    provides the artifact ref and file content in a single multipart request. The server:

    1. Looks up the artifact by `ref`.
    2. If not found, creates it using the metadata fields in the multipart body.
    3. If found, optionally updates the `execution` link to the current execution.
    4. Uploads the file bytes as a new version (version number is auto-assigned).

    **Multipart fields:**
    - `file` (required) — the binary file content
    - `ref` (required for creation) — artifact reference (ignored if artifact already exists)
    - `scope` — owner scope: `system`, `pack`, `action`, `sensor`, `rule` (default: `action`)
    - `owner` — owner identifier (default: empty string)
    - `type` — artifact type: `file_text`, `file_image`, etc. (default: `file_text`)
    - `visibility` — `public` or `private` (default: type-aware server default)
    - `name` — human-readable name
    - `description` — optional description
    - `content_type` — MIME type (default: auto-detected from multipart or `application/octet-stream`)
    - `execution` — execution ID to link this artifact to (updates existing artifacts too)
    - `retention_policy` — `versions`, `days`, `hours`, `minutes` (default: `versions`)
    - `retention_limit` — limit value (default: `10`)
    - `created_by` — who created this version
    - `meta` — JSON metadata for this version

    Args:
        ref (str):
        body (ArtifactVersionByRefUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UploadVersionByRefResponse201]
    """

    kwargs = _get_kwargs(
        ref=ref,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionByRefUploadForm,
) -> Any | UploadVersionByRefResponse201 | None:
    """Upload a file version to an artifact identified by ref, creating the artifact if it does not
    already exist.

     This is the recommended way for actions to produce versioned file artifacts. The caller
    provides the artifact ref and file content in a single multipart request. The server:

    1. Looks up the artifact by `ref`.
    2. If not found, creates it using the metadata fields in the multipart body.
    3. If found, optionally updates the `execution` link to the current execution.
    4. Uploads the file bytes as a new version (version number is auto-assigned).

    **Multipart fields:**
    - `file` (required) — the binary file content
    - `ref` (required for creation) — artifact reference (ignored if artifact already exists)
    - `scope` — owner scope: `system`, `pack`, `action`, `sensor`, `rule` (default: `action`)
    - `owner` — owner identifier (default: empty string)
    - `type` — artifact type: `file_text`, `file_image`, etc. (default: `file_text`)
    - `visibility` — `public` or `private` (default: type-aware server default)
    - `name` — human-readable name
    - `description` — optional description
    - `content_type` — MIME type (default: auto-detected from multipart or `application/octet-stream`)
    - `execution` — execution ID to link this artifact to (updates existing artifacts too)
    - `retention_policy` — `versions`, `days`, `hours`, `minutes` (default: `versions`)
    - `retention_limit` — limit value (default: `10`)
    - `created_by` — who created this version
    - `meta` — JSON metadata for this version

    Args:
        ref (str):
        body (ArtifactVersionByRefUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UploadVersionByRefResponse201
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            body=body,
        )
    ).parsed
