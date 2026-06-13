from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_work_queue_item_response import (
    ApiResponseWorkQueueItemResponse,
)
from ...models.update_work_queue_item_request import UpdateWorkQueueItemRequest
from ...types import Response


def _get_kwargs(
    ref: str,
    item_id: int,
    *,
    body: UpdateWorkQueueItemRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/queues/{ref}/items/{item_id}".format(
            ref=quote(str(ref), safe=""),
            item_id=quote(str(item_id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseWorkQueueItemResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseWorkQueueItemResponse.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 403:
        response_403 = cast(Any, None)
        return response_403

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 409:
        response_409 = cast(Any, None)
        return response_409

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ApiResponseWorkQueueItemResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    ref: str,
    item_id: int,
    *,
    client: AuthenticatedClient,
    body: UpdateWorkQueueItemRequest,
) -> Response[Any | ApiResponseWorkQueueItemResponse]:
    """
    Args:
        ref (str):
        item_id (int):
        body (UpdateWorkQueueItemRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseWorkQueueItemResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        item_id=item_id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    item_id: int,
    *,
    client: AuthenticatedClient,
    body: UpdateWorkQueueItemRequest,
) -> Any | ApiResponseWorkQueueItemResponse | None:
    """
    Args:
        ref (str):
        item_id (int):
        body (UpdateWorkQueueItemRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseWorkQueueItemResponse
    """

    return sync_detailed(
        ref=ref,
        item_id=item_id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    ref: str,
    item_id: int,
    *,
    client: AuthenticatedClient,
    body: UpdateWorkQueueItemRequest,
) -> Response[Any | ApiResponseWorkQueueItemResponse]:
    """
    Args:
        ref (str):
        item_id (int):
        body (UpdateWorkQueueItemRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseWorkQueueItemResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        item_id=item_id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    item_id: int,
    *,
    client: AuthenticatedClient,
    body: UpdateWorkQueueItemRequest,
) -> Any | ApiResponseWorkQueueItemResponse | None:
    """
    Args:
        ref (str):
        item_id (int):
        body (UpdateWorkQueueItemRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseWorkQueueItemResponse
    """

    return (
        await asyncio_detailed(
            ref=ref,
            item_id=item_id,
            client=client,
            body=body,
        )
    ).parsed
