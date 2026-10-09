from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.append_progress_request import AppendProgressRequest
from ...models.append_progress_response_200 import AppendProgressResponse200
from ...types import Response


def _get_kwargs(
    id: int,
    *,
    body: AppendProgressRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/artifacts/{id}/progress".format(
            id=quote(str(id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | AppendProgressResponse200 | None:
    if response.status_code == 200:
        response_200 = AppendProgressResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | AppendProgressResponse200]:
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
    body: AppendProgressRequest,
) -> Response[Any | AppendProgressResponse200]:
    """Append an entry to a progress-type artifact's data array.

     The entry is atomically appended to `artifact.data` (initialized as `[]` if null).
    This is the primary mechanism for actions to stream progress updates.

    Args:
        id (int):
        body (AppendProgressRequest): Request DTO for appending to a progress-type artifact

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | AppendProgressResponse200]
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
    body: AppendProgressRequest,
) -> Any | AppendProgressResponse200 | None:
    """Append an entry to a progress-type artifact's data array.

     The entry is atomically appended to `artifact.data` (initialized as `[]` if null).
    This is the primary mechanism for actions to stream progress updates.

    Args:
        id (int):
        body (AppendProgressRequest): Request DTO for appending to a progress-type artifact

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | AppendProgressResponse200
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
    body: AppendProgressRequest,
) -> Response[Any | AppendProgressResponse200]:
    """Append an entry to a progress-type artifact's data array.

     The entry is atomically appended to `artifact.data` (initialized as `[]` if null).
    This is the primary mechanism for actions to stream progress updates.

    Args:
        id (int):
        body (AppendProgressRequest): Request DTO for appending to a progress-type artifact

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | AppendProgressResponse200]
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
    body: AppendProgressRequest,
) -> Any | AppendProgressResponse200 | None:
    """Append an entry to a progress-type artifact's data array.

     The entry is atomically appended to `artifact.data` (initialized as `[]` if null).
    This is the primary mechanism for actions to stream progress updates.

    Args:
        id (int):
        body (AppendProgressRequest): Request DTO for appending to a progress-type artifact

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | AppendProgressResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
