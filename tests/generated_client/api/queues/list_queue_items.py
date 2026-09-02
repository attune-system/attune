from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_work_queue_item_response import (
    PaginatedResponseWorkQueueItemResponse,
)
from ...models.work_queue_item_status import WorkQueueItemStatus
from ...types import UNSET, Response, Unset


def _get_kwargs(
    ref: str,
    *,
    item_key: str | Unset = UNSET,
    enqueue_source: str | Unset = UNSET,
    statuses: list[WorkQueueItemStatus] | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["item_key"] = item_key

    params["enqueue_source"] = enqueue_source

    json_statuses: list[str] | Unset = UNSET
    if not isinstance(statuses, Unset):
        json_statuses = []
        for statuses_item_data in statuses:
            statuses_item = statuses_item_data.value
            json_statuses.append(statuses_item)

    params["statuses"] = json_statuses

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/queues/{ref}/items".format(
            ref=quote(str(ref), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseWorkQueueItemResponse | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseWorkQueueItemResponse.from_dict(response.json())

        return response_200

    if response.status_code == 403:
        response_403 = cast(Any, None)
        return response_403

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | PaginatedResponseWorkQueueItemResponse]:
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
    item_key: str | Unset = UNSET,
    enqueue_source: str | Unset = UNSET,
    statuses: list[WorkQueueItemStatus] | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseWorkQueueItemResponse]:
    """
    Args:
        ref (str):
        item_key (str | Unset):
        enqueue_source (str | Unset):
        statuses (list[WorkQueueItemStatus] | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseWorkQueueItemResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        item_key=item_key,
        enqueue_source=enqueue_source,
        statuses=statuses,
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient,
    item_key: str | Unset = UNSET,
    enqueue_source: str | Unset = UNSET,
    statuses: list[WorkQueueItemStatus] | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseWorkQueueItemResponse | None:
    """
    Args:
        ref (str):
        item_key (str | Unset):
        enqueue_source (str | Unset):
        statuses (list[WorkQueueItemStatus] | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseWorkQueueItemResponse
    """

    return sync_detailed(
        ref=ref,
        client=client,
        item_key=item_key,
        enqueue_source=enqueue_source,
        statuses=statuses,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    item_key: str | Unset = UNSET,
    enqueue_source: str | Unset = UNSET,
    statuses: list[WorkQueueItemStatus] | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseWorkQueueItemResponse]:
    """
    Args:
        ref (str):
        item_key (str | Unset):
        enqueue_source (str | Unset):
        statuses (list[WorkQueueItemStatus] | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseWorkQueueItemResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        item_key=item_key,
        enqueue_source=enqueue_source,
        statuses=statuses,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient,
    item_key: str | Unset = UNSET,
    enqueue_source: str | Unset = UNSET,
    statuses: list[WorkQueueItemStatus] | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseWorkQueueItemResponse | None:
    """
    Args:
        ref (str):
        item_key (str | Unset):
        enqueue_source (str | Unset):
        statuses (list[WorkQueueItemStatus] | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseWorkQueueItemResponse
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            item_key=item_key,
            enqueue_source=enqueue_source,
            statuses=statuses,
            page=page,
            per_page=per_page,
        )
    ).parsed
