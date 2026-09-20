from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: int,
    stream: str,
    *,
    offset: int | Unset = UNSET,
    last_event_id: int | None | Unset = UNSET,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}
    if not isinstance(last_event_id, Unset):
        headers["Last-Event-ID"] = last_event_id

    params: dict[str, Any] = {}

    params["offset"] = offset

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/executions/{id}/logs/{stream}/stream".format(
            id=quote(str(id), safe=""),
            stream=quote(str(stream), safe=""),
        ),
        "params": params,
    }

    _kwargs["headers"] = headers
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

    if response.status_code == 429:
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
    stream: str,
    *,
    client: AuthenticatedClient | Client,
    offset: int | Unset = UNSET,
    last_event_id: int | None | Unset = UNSET,
) -> Response[Any]:
    """Stream stdout/stderr for an execution as SSE.

     This tails the stream backend selected when the worker allocates its log
    artifacts. The stream may not exist yet while allocation is pending.
    An explicit `offset` query parameter takes precedence over `Last-Event-ID`.

    Args:
        id (int):
        stream (str):
        offset (int | Unset):
        last_event_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
        stream=stream,
        offset=offset,
        last_event_id=last_event_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


async def asyncio_detailed(
    id: int,
    stream: str,
    *,
    client: AuthenticatedClient | Client,
    offset: int | Unset = UNSET,
    last_event_id: int | None | Unset = UNSET,
) -> Response[Any]:
    """Stream stdout/stderr for an execution as SSE.

     This tails the stream backend selected when the worker allocates its log
    artifacts. The stream may not exist yet while allocation is pending.
    An explicit `offset` query parameter takes precedence over `Last-Event-ID`.

    Args:
        id (int):
        stream (str):
        offset (int | Unset):
        last_event_id (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any]
    """

    kwargs = _get_kwargs(
        id=id,
        stream=stream,
        offset=offset,
        last_event_id=last_event_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)
