from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.artifact_version_upload_form import ArtifactVersionUploadForm
from ...models.upload_version_response_201 import UploadVersionResponse201
from ...types import Response


def _get_kwargs(
    id: int,
    *,
    body: ArtifactVersionUploadForm,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/artifacts/{id}/versions/upload".format(
            id=quote(str(id), safe=""),
        ),
    }

    _kwargs["files"] = body.to_multipart()

    headers["Content-Type"] = "multipart/form-data; boundary=+++"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | UploadVersionResponse201 | None:
    if response.status_code == 201:
        response_201 = UploadVersionResponse201.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 413:
        response_413 = cast(Any, None)
        return response_413

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | UploadVersionResponse201]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionUploadForm,
) -> Response[Any | UploadVersionResponse201]:
    """Upload a binary file as a new version (multipart/form-data)

     The file is sent as a multipart form field named `file`. Optional fields:
    - `content_type`: MIME type override (auto-detected from filename if omitted)
    - `meta`: JSON metadata string
    - `created_by`: Creator identifier

    Args:
        id (int):
        body (ArtifactVersionUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UploadVersionResponse201]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionUploadForm,
) -> Any | UploadVersionResponse201 | None:
    """Upload a binary file as a new version (multipart/form-data)

     The file is sent as a multipart form field named `file`. Optional fields:
    - `content_type`: MIME type override (auto-detected from filename if omitted)
    - `meta`: JSON metadata string
    - `created_by`: Creator identifier

    Args:
        id (int):
        body (ArtifactVersionUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UploadVersionResponse201
    """

    return sync_detailed(
        id=id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionUploadForm,
) -> Response[Any | UploadVersionResponse201]:
    """Upload a binary file as a new version (multipart/form-data)

     The file is sent as a multipart form field named `file`. Optional fields:
    - `content_type`: MIME type override (auto-detected from filename if omitted)
    - `meta`: JSON metadata string
    - `created_by`: Creator identifier

    Args:
        id (int):
        body (ArtifactVersionUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UploadVersionResponse201]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    *,
    client: AuthenticatedClient,
    body: ArtifactVersionUploadForm,
) -> Any | UploadVersionResponse201 | None:
    """Upload a binary file as a new version (multipart/form-data)

     The file is sent as a multipart form field named `file`. Optional fields:
    - `content_type`: MIME type override (auto-detected from filename if omitted)
    - `meta`: JSON metadata string
    - `created_by`: Creator identifier

    Args:
        id (int):
        body (ArtifactVersionUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UploadVersionResponse201
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
