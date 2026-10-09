from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...types import Response


def _get_kwargs(
    id: int,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/artifacts/{id}/stream".format(
            id=quote(str(id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | None:
    if response.status_code == 200:
        return None

    if response.status_code == 401:
        return None

    if response.status_code == 404:
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
    *,
    client: AuthenticatedClient | Client,
) -> Response[Any]:
    """Stream the latest file-backed artifact version as Server-Sent Events.

     The endpoint:
    1. Streams committed immutable log segments when the version has an
       object-segment log stream.
    2. Otherwise, waits for the allocated file and tails appended bytes.
    3. When no new bytes have appeared for several consecutive checks **and** the
       linked execution (if any) has reached a terminal status, it sends a `done`
       event and the stream ends.
    4. If the client disconnects the stream is cleaned up automatically.

    **Event types** (SSE `event:` field):
    - `content`  – full file content up to the current offset (sent once)
    - `append`   – incremental bytes appended since the last event
    - `waiting`  – file does not exist yet; sent periodically while waiting
    - `done`     – no more data expected; stream will close
    - `error`    – something went wrong; `data` contains a human-readable message

    Args:
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient | Client,
) -> Response[Any]:
    """Stream the latest file-backed artifact version as Server-Sent Events.

     The endpoint:
    1. Streams committed immutable log segments when the version has an
       object-segment log stream.
    2. Otherwise, waits for the allocated file and tails appended bytes.
    3. When no new bytes have appeared for several consecutive checks **and** the
       linked execution (if any) has reached a terminal status, it sends a `done`
       event and the stream ends.
    4. If the client disconnects the stream is cleaned up automatically.

    **Event types** (SSE `event:` field):
    - `content`  – full file content up to the current offset (sent once)
    - `append`   – incremental bytes appended since the last event
    - `waiting`  – file does not exist yet; sent periodically while waiting
    - `done`     – no more data expected; stream will close
    - `error`    – something went wrong; `data` contains a human-readable message

    Args:
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)
