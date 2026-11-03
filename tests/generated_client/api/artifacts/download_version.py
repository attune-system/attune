from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: int,
    version: int,
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
        "url": "/api/v1/artifacts/{id}/versions/{version}/download".format(
            id=quote(str(id), safe=""),
            version=quote(str(version), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | None:
    if response.status_code == 200:
        return None

    if response.status_code == 206:
        return None

    if response.status_code == 404:
        return None

    if response.status_code == 416:
        return None

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any]:
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
    version_id: int | None | Unset = UNSET,
) -> Response[Any]:
    """Download the binary content of a specific version.

     For file-backed versions, reads from the shared artifact volume on disk.
    For DB-stored versions, reads from the BYTEA/JSON content column.

    Args:
        id (int):
        version (int):
        version_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
        version=version,
        version_id=version_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


async def asyncio_detailed(
    id: int,
    version: int,
    *,
    client: AuthenticatedClient,
    version_id: int | None | Unset = UNSET,
) -> Response[Any]:
    """Download the binary content of a specific version.

     For file-backed versions, reads from the shared artifact volume on disk.
    For DB-stored versions, reads from the BYTEA/JSON content column.

    Args:
        id (int):
        version (int):
        version_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
        version=version,
        version_id=version_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)
