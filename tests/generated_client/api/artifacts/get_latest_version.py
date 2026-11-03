from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.get_latest_version_response_200 import GetLatestVersionResponse200
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: int,
    *,
    version_id: int | None | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_version_id: int | None | Unset
    if isinstance(version_id, Unset):
        json_version_id = UNSET
    else:
        json_version_id = version_id
    params["version_id"] = json_version_id

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/artifacts/{id}/versions/latest".format(
            id=quote(str(id), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | GetLatestVersionResponse200 | None:
    if response.status_code == 200:
        response_200 = GetLatestVersionResponse200.from_dict(response.json())

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
) -> Response[Any | GetLatestVersionResponse200]:
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
    version_id: int | None | Unset = UNSET,
) -> Response[Any | GetLatestVersionResponse200]:
    """Get the latest version's metadata and JSON content

    Args:
        id (int):
        version_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | GetLatestVersionResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        version_id=version_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    *,
    client: AuthenticatedClient,
    version_id: int | None | Unset = UNSET,
) -> Any | GetLatestVersionResponse200 | None:
    """Get the latest version's metadata and JSON content

    Args:
        id (int):
        version_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | GetLatestVersionResponse200
    """

    return sync_detailed(
        id=id,
        client=client,
        version_id=version_id,
    ).parsed


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    version_id: int | None | Unset = UNSET,
) -> Response[Any | GetLatestVersionResponse200]:
    """Get the latest version's metadata and JSON content

    Args:
        id (int):
        version_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | GetLatestVersionResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        version_id=version_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    *,
    client: AuthenticatedClient,
    version_id: int | None | Unset = UNSET,
) -> Any | GetLatestVersionResponse200 | None:
    """Get the latest version's metadata and JSON content

    Args:
        id (int):
        version_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | GetLatestVersionResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            version_id=version_id,
        )
    ).parsed
