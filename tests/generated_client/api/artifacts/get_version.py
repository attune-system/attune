from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.get_version_response_200 import GetVersionResponse200
from ...types import Response


def _get_kwargs(
    id: int,
    version: int,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/artifacts/{id}/versions/{version}".format(
            id=quote(str(id), safe=""),
            version=quote(str(version), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | GetVersionResponse200 | None:
    if response.status_code == 200:
        response_200 = GetVersionResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | GetVersionResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: int,
    version: int,
    *,
    client: AuthenticatedClient,
) -> Response[Any | GetVersionResponse200]:
    """Get a specific version's metadata and JSON content (no binary)

    Args:
        id (int):
        version (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | GetVersionResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        version=version,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    version: int,
    *,
    client: AuthenticatedClient,
) -> Any | GetVersionResponse200 | None:
    """Get a specific version's metadata and JSON content (no binary)

    Args:
        id (int):
        version (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | GetVersionResponse200
    """

    return sync_detailed(
        id=id,
        version=version,
        client=client,
    ).parsed


async def asyncio_detailed(
    id: int,
    version: int,
    *,
    client: AuthenticatedClient,
) -> Response[Any | GetVersionResponse200]:
    """Get a specific version's metadata and JSON content (no binary)

    Args:
        id (int):
        version (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | GetVersionResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        version=version,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    version: int,
    *,
    client: AuthenticatedClient,
) -> Any | GetVersionResponse200 | None:
    """Get a specific version's metadata and JSON content (no binary)

    Args:
        id (int):
        version (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | GetVersionResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            version=version,
            client=client,
        )
    ).parsed
